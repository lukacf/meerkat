//! The audit rows of scoped tool grants.
//!
//! Each subject's grants append to one audit log, numbered 0, 1, 2, ... in
//! append order. A lifecycle row (`granted`, `expired`, `revoked`) commits
//! in the same store transaction as the grant row change it records. A
//! `refused` row is appended after the refusal was decided: a failed append
//! is logged and never turns a refusal into an admission.

use meerkat_core::tool_grant::{ToolGrantId, ToolGrantIssuer, ToolGrantRefusal, ToolGrantTarget};
use meerkat_core::types::ToolName;
use serde::{Deserialize, Serialize};

/// Which driver observed a grant's expiry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolGrantExpiryDriver {
    /// The served session's deadline timer.
    Timer,
    /// A model-request boundary.
    Boundary,
    /// A gated dispatch.
    Dispatch,
    /// A list or audit read.
    Read,
    /// A revoke at or after the deadline.
    Revoke,
}

/// Why a gated call was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolGrantRefusalKind {
    /// No live grant covers the name.
    Required,
    /// The grant covering the name reached its deadline.
    Expired,
    /// The grant covering the name was revoked.
    Revoked,
    /// The grants could not be read or settled.
    Unavailable,
}

impl ToolGrantRefusalKind {
    /// The kind of `refusal`.
    #[must_use]
    pub fn of(refusal: &ToolGrantRefusal) -> Self {
        match refusal {
            ToolGrantRefusal::Required => Self::Required,
            ToolGrantRefusal::Expired { .. } => Self::Expired,
            ToolGrantRefusal::Revoked { .. } => Self::Revoked,
            ToolGrantRefusal::Unavailable { .. } => Self::Unavailable,
        }
    }
}

/// What one audit row records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ToolGrantAuditEvent {
    /// A grant was issued.
    Granted {
        grant_id: ToolGrantId,
        target: ToolGrantTarget,
        issuer: ToolGrantIssuer,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        expires_at_ms: u64,
    },
    /// A grant was observed at or after its deadline.
    Expired {
        grant_id: ToolGrantId,
        expires_at_ms: u64,
        observed_at_ms: u64,
        driver: ToolGrantExpiryDriver,
    },
    /// A grant was revoked before its deadline.
    Revoked {
        grant_id: ToolGrantId,
        observed_at_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// A gated call was refused.
    Refused {
        tool: ToolName,
        reason: ToolGrantRefusalKind,
        /// The grant that explains the refusal, when one does.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        grant_id: Option<ToolGrantId>,
        call_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<String>,
    },
}

/// One audit row as it is appended: when it was recorded and what it
/// records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolGrantAuditRow {
    /// The host clock reading the row was recorded at, in UNIX ms.
    pub at_ms: u64,
    pub event: ToolGrantAuditEvent,
}

impl ToolGrantAuditRow {
    /// The row's stored bytes.
    ///
    /// # Errors
    /// The JSON encoder's failure.
    pub fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    /// A row read back from its stored bytes.
    ///
    /// # Errors
    /// The JSON decoder's failure.
    pub fn decode(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}

/// One audit row as a read returns it: its place in its subject's log and
/// the row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolGrantAuditRecord {
    /// The row's place among its subject's rows: 0 for the first.
    pub sequence: u64,
    pub row: ToolGrantAuditRow,
}

/// An audit row this binary cannot read (written by a newer binary, or
/// damaged): its place in its subject's log and why it does not decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreadableToolGrantAuditRow {
    pub sequence: u64,
    pub detail: String,
}

/// One page of a subject's audit log: the rows this binary reads, and the
/// rows of the page it cannot read. An unreadable row never hides the rest
/// of its page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolGrantAuditPage {
    pub rows: Vec<ToolGrantAuditRecord>,
    pub unreadable: Vec<UnreadableToolGrantAuditRow>,
}

impl ToolGrantAuditPage {
    /// How many rows of the log the page holds, readable or not.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rows.len() + self.unreadable.len()
    }

    /// Whether the page holds no row of the log.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty() && self.unreadable.is_empty()
    }

    /// The last sequence the page holds, readable or not: the cursor of the
    /// next page.
    #[must_use]
    pub fn last_sequence(&self) -> Option<u64> {
        self.rows
            .iter()
            .map(|record| record.sequence)
            .chain(self.unreadable.iter().map(|row| row.sequence))
            .max()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn rows_round_trip_and_refuse_unknown_fields() {
        let row = ToolGrantAuditRow {
            at_ms: 7,
            event: ToolGrantAuditEvent::Refused {
                tool: ToolName::from("shell"),
                reason: ToolGrantRefusalKind::Expired,
                grant_id: Some(ToolGrantId::mint()),
                call_id: "call-1".into(),
                run_id: None,
            },
        };
        let bytes = row.encode().unwrap();
        assert_eq!(ToolGrantAuditRow::decode(&bytes).unwrap(), row);

        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        value["event"]["surprise"] = serde_json::json!(true);
        assert!(ToolGrantAuditRow::decode(&serde_json::to_vec(&value).unwrap()).is_err());
    }
}

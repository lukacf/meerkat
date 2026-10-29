//! Scoped tool grant wire contracts: the `tool_grants/*` JSON-RPC methods.
//!
//! A build marks tool families and names grant-gated; a scoped tool grant
//! opens, for a bounded time, the gated names its target covers for its
//! subject. The grant lifecycle (issuance, expiry, revocation, use
//! admission) is owned by the runtime's `ToolGrantLifecycleMachine`; these
//! types are its surface projection. Tagged enums use `kind`, and every
//! closed vocabulary a later version may extend (status, issuer, audit event,
//! expiry driver, refusal kind) carries a forward-compatible `unknown`
//! variant that the server never sends.

use serde::{Deserialize, Serialize};

/// A session build's scoped tool grant gate (`session/create`
/// `grant_gated_tools`): the built-in tool families and the tool names the
/// build mounts hidden and refuses at dispatch until a live grant opens them.
/// A field this version does not know is refused, never ignored.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct WireGrantGatedTools {
    /// Built-in tool families, such as `shell`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub families: Vec<String>,
    /// Tool names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
}

impl From<WireGrantGatedTools> for meerkat_core::tool_grant::GrantGatedTools {
    fn from(wire: WireGrantGatedTools) -> Self {
        let mut gate = Self::default();
        gate.families.extend(wire.families);
        for tool in wire.tools {
            gate.tools.insert(tool);
        }
        gate
    }
}

/// Whose grants: a session, or (in a later version) a mob member's identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireToolGrantSubject {
    /// One session's grants.
    Session { session_id: String },
    /// One mob member identity's grants. Refused by servers that do not
    /// support identity subjects.
    Identity { mob_id: String, identity: String },
}

/// What a grant opens: one tool name, or a built-in tool family.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireToolGrantTarget {
    /// One tool name.
    Tool { name: String },
    /// A built-in tool family. `names` is the family's snapshot the server
    /// takes at grant time: omit it in a request.
    Family {
        family: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        names: Vec<String>,
    },
}

/// Who issued a grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireToolGrantIssuer {
    /// An operator, through a host surface.
    Operator {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caller: Option<String>,
    },
    /// An approval decision.
    Approval { approval_id: String },
    /// Forward compatibility: an issuer kind this version does not know.
    /// The server never sends it.
    #[serde(other)]
    Unknown,
}

/// A grant's status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireToolGrantStatus {
    /// Issued, and not yet observed at or after its deadline.
    Active,
    /// Observed at or after its deadline. Terminal.
    Expired,
    /// Revoked before its deadline. Terminal.
    Revoked,
    /// Requested through an approval, not yet decided.
    Requested,
    /// Requested through an approval and declined. Terminal.
    Declined,
    /// Forward compatibility: a status this version does not know. The
    /// server never sends it.
    #[serde(other)]
    Unknown,
}

/// One scoped tool grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireToolGrant {
    pub grant_id: String,
    pub subject: WireToolGrantSubject,
    pub target: WireToolGrantTarget,
    pub issuer: WireToolGrantIssuer,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub status: WireToolGrantStatus,
    /// The host clock reading the grant was issued at, in UNIX ms.
    pub issued_at_ms: u64,
    /// The grant's fixed deadline, in UNIX ms.
    pub expires_at_ms: u64,
    /// The host clock reading the grant was observed to end at, in UNIX ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at_ms: Option<u64>,
}

/// A stored grant row the server cannot read (written by a newer version,
/// or damaged). It opens nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireUnreadableToolGrant {
    pub grant_id: String,
    pub detail: String,
}

/// `tool_grants/grant` parameters. Exactly one of `expires_at_ms` and
/// `duration_ms` bounds the grant, at most the host's maximum lifetime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct GrantToolParams {
    pub subject: WireToolGrantSubject,
    pub target: WireToolGrantTarget,
    /// The grant's absolute deadline, in UNIX ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<u64>,
    /// The grant's lifetime from the server's clock, in ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `tool_grants/grant` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GrantToolResult {
    pub grant: WireToolGrant,
}

/// `tool_grants/revoke` parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RevokeToolGrantParams {
    pub grant_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `tool_grants/revoke` result. A revoke is idempotent: `changed` says
/// whether this revoke ended the grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RevokeToolGrantResult {
    pub grant: WireToolGrant,
    pub changed: bool,
}

/// `tool_grants/list` parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ListToolGrantsParams {
    pub subject: WireToolGrantSubject,
    /// List ended (expired or revoked) grants too.
    #[serde(default)]
    pub include_ended: bool,
}

/// `tool_grants/list` result. Due grants are settled as expired first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ListToolGrantsResult {
    pub grants: Vec<WireToolGrant>,
    /// Rows of the subject the server cannot read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unreadable: Vec<WireUnreadableToolGrant>,
}

/// `tool_grants/audit` parameters: a page of the subject's audit log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ToolGrantAuditParams {
    pub subject: WireToolGrantSubject,
    /// Rows after this sequence; the log's start when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_sequence: Option<u64>,
    /// At most this many rows; the server's default when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Which driver observed a grant's expiry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireToolGrantExpiryDriver {
    Timer,
    Boundary,
    Dispatch,
    Read,
    Revoke,
    /// Forward compatibility: a driver this version does not know. The
    /// server never sends it.
    #[serde(other)]
    Unknown,
}

/// Why a gated call was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireToolGrantRefusalKind {
    Required,
    Expired,
    Revoked,
    Unavailable,
    /// Forward compatibility: a refusal kind this version does not know.
    /// The server never sends it.
    #[serde(other)]
    Unknown,
}

/// One audit event of a subject's grants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireToolGrantAuditEvent {
    Granted {
        grant_id: String,
        target: WireToolGrantTarget,
        issuer: WireToolGrantIssuer,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        expires_at_ms: u64,
    },
    Expired {
        grant_id: String,
        expires_at_ms: u64,
        observed_at_ms: u64,
        driver: WireToolGrantExpiryDriver,
    },
    Revoked {
        grant_id: String,
        observed_at_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Refused {
        tool: String,
        reason: WireToolGrantRefusalKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        grant_id: Option<String>,
        call_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<String>,
    },
    /// Forward compatibility: an event kind this version does not know. The
    /// server never sends it.
    #[serde(other)]
    Unknown,
}

/// One audit row: its place in the subject's log, when it was recorded, and
/// its event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireToolGrantAuditRow {
    pub sequence: u64,
    /// The host clock reading the row was recorded at, in UNIX ms.
    pub at_ms: u64,
    pub event: WireToolGrantAuditEvent,
}

/// `tool_grants/audit` result. `next_sequence` is the `after_sequence` of
/// the next page while more rows may follow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ToolGrantAuditResult {
    pub rows: Vec<WireToolGrantAuditRow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_sequence: Option<u64>,
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn grant_params_tag_their_subject_and_target_and_refuse_unknown_fields() {
        let params = GrantToolParams {
            subject: WireToolGrantSubject::Session {
                session_id: "session-1".to_string(),
            },
            target: WireToolGrantTarget::Family {
                family: "shell".to_string(),
                names: Vec::new(),
            },
            expires_at_ms: None,
            duration_ms: Some(60_000),
            reason: None,
        };
        let value = serde_json::to_value(&params).unwrap();
        assert_eq!(
            value,
            json!({
                "subject": {"kind": "session", "session_id": "session-1"},
                "target": {"kind": "family", "family": "shell"},
                "duration_ms": 60_000,
            })
        );
        assert_eq!(
            serde_json::from_value::<GrantToolParams>(value).unwrap(),
            params
        );
        assert!(
            serde_json::from_value::<GrantToolParams>(json!({
                "subject": {"kind": "session", "session_id": "session-1"},
                "target": {"kind": "tool", "name": "shell"},
                "duration_ms": 1,
                "scope": "current_run",
            }))
            .is_err()
        );
    }

    #[test]
    fn unknown_status_issuer_and_event_kinds_decode_as_unknown() {
        let grant: WireToolGrant = serde_json::from_value(json!({
            "grant_id": "g1",
            "subject": {"kind": "session", "session_id": "session-1"},
            "target": {"kind": "tool", "name": "deploy"},
            "issuer": {"kind": "delegated", "by": "someone"},
            "status": "suspended",
            "issued_at_ms": 1,
            "expires_at_ms": 2,
        }))
        .unwrap();
        assert_eq!(grant.status, WireToolGrantStatus::Unknown);
        assert_eq!(grant.issuer, WireToolGrantIssuer::Unknown);

        let row: WireToolGrantAuditRow = serde_json::from_value(json!({
            "sequence": 4,
            "at_ms": 9,
            "event": {"kind": "activated", "grant_id": "g1"},
        }))
        .unwrap();
        assert_eq!(row.event, WireToolGrantAuditEvent::Unknown);

        let expired: WireToolGrantAuditEvent = serde_json::from_value(json!({
            "kind": "expired",
            "grant_id": "g1",
            "expires_at_ms": 2,
            "observed_at_ms": 3,
            "driver": "watchdog",
        }))
        .unwrap();
        assert!(matches!(
            expired,
            WireToolGrantAuditEvent::Expired {
                driver: WireToolGrantExpiryDriver::Unknown,
                ..
            }
        ));
    }
}

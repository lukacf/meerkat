//! Approval protocol wire contracts.
//!
//! The durable approval domain types live in `meerkat-core`; this module
//! re-exports them as the canonical wire shape and adds surface request
//! wrappers for JSON-RPC/REST methods.

use serde::{Deserialize, Serialize};

pub use meerkat_core::{
    ApprovalActionKind, ApprovalDecision, ApprovalDecisionRecord, ApprovalId, ApprovalListFilter,
    ApprovalOwnerRef, ApprovalPrincipalId, ApprovalProposedAction, ApprovalRecord, ApprovalRequest,
    ApprovalResourceKind, ApprovalResourceRef, ApprovalRisk, ApprovalStatus,
};

/// `approval/request` parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ApprovalRequestParams {
    #[serde(flatten)]
    pub request: ApprovalRequest,
}

/// `approval/get` parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ApprovalGetParams {
    pub approval_id: ApprovalId,
}

/// `approval/list` parameters.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ApprovalListParams {
    #[serde(default)]
    pub filter: ApprovalListFilter,
}

/// `approval/list` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ApprovalListResult {
    pub approvals: Vec<ApprovalRecord>,
}

/// `approval/decide` parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ApprovalDecideParams {
    pub approval_id: ApprovalId,
    pub decision: ApprovalDecision,
    pub actor: ApprovalPrincipalId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<serde_json::Value>,
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::{BTreeMap, BTreeSet};

    fn principal(value: &str) -> ApprovalPrincipalId {
        ApprovalPrincipalId::new(value).expect("valid principal")
    }

    #[test]
    fn approval_request_uses_typed_status_decision_and_metadata_fields() {
        let params = ApprovalRequestParams {
            request: ApprovalRequest {
                requester: principal("human:alice"),
                owner: ApprovalOwnerRef::Session {
                    session_id: "session-1".to_string(),
                },
                resource: ApprovalResourceRef {
                    kind: ApprovalResourceKind::ShellCommand,
                    id: "shell:danger".to_string(),
                },
                proposed_action: ApprovalProposedAction {
                    kind: ApprovalActionKind::ShellCommand,
                    summary: "run shell command".to_string(),
                    body: Some(json!({"cmd": "rm -rf target/tmp"})),
                },
                risk: ApprovalRisk::High,
                request_body: Some(json!({"why": "cleanup"})),
                allowed_decisions: BTreeSet::from([
                    ApprovalDecision::Approve,
                    ApprovalDecision::Deny,
                ]),
                expires_at: None,
                metadata: meerkat_core::SurfaceMetadata {
                    labels: BTreeMap::from([(
                        "client.thread_id".to_string(),
                        "thread-1".to_string(),
                    )]),
                    app_context: Some(json!({"client_ref": "opaque"})),
                },
                request_provenance: Some(json!({"tool_call_id": "call-1"})),
            },
        };

        let value = serde_json::to_value(&params).expect("serialize approval request");
        assert_eq!(value["requester"], "human:alice");
        assert_eq!(value["owner"]["owner_type"], "session");
        assert_eq!(value["resource"]["kind"], "shell_command");
        assert_eq!(value["allowed_decisions"][0], "approve");
        assert_eq!(value["metadata"]["labels"]["client.thread_id"], "thread-1");
        let parsed: ApprovalRequestParams =
            serde_json::from_value(value).expect("deserialize approval request");
        assert_eq!(parsed, params);
    }

    #[test]
    fn approval_decide_uses_typed_decision_and_actor() {
        let params = ApprovalDecideParams {
            approval_id: ApprovalId::new(),
            decision: ApprovalDecision::Deny,
            actor: principal("human:bob"),
            reason: Some("too risky".to_string()),
            provenance: Some(json!({"client": "mobile"})),
        };

        let value = serde_json::to_value(&params).expect("serialize decision");
        assert_eq!(value["decision"], "deny");
        assert_eq!(value["actor"], "human:bob");
        let parsed: ApprovalDecideParams =
            serde_json::from_value(value).expect("deserialize decision");
        assert_eq!(parsed, params);
    }
}

//! The wire of Meta's decision verbs: Meta's list answer with its status
//! rows, the status view, the decide request with its attestation, the
//! notice, and the JSON-RPC answers. A decision read whole, a rendering and
//! the refusal vocabulary are Core's `decision/*` contracts, unchanged.

use meerkat_contracts::{
    DecisionPresentationRequest, ErrorCode, WireDecision, WireDecisionErrorData,
    WireDecisionNoticePresentation, WireDecisionPhase, WireUnreadableDecision,
};
use meerkat_core::approval::decision::StoredDecision;
use meerkat_runtime::decision::DecisionError;
use meerkat_runtime::decision::wire::{wire_decision_phase, wire_decision_refusal};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The prefix of every decision verb.
pub const DECISIONS_METHOD_PREFIX: &str = "meta/decisions/";
/// The notification the stdio host receives for a held decision.
pub const DECISION_NOTICE_METHOD: &str = "meta/decisions/notice";

pub(crate) const LIST_METHOD: &str = "meta/decisions/list";
pub(crate) const GET_METHOD: &str = "meta/decisions/get";
pub(crate) const RENDERING_METHOD: &str = "meta/decisions/rendering";
pub(crate) const PRESENT_METHOD: &str = "meta/decisions/present";
pub(crate) const DECIDE_METHOD: &str = "meta/decisions/decide";
pub(crate) const WITHDRAW_METHOD: &str = "meta/decisions/withdraw";

pub(crate) const METHOD_NOT_FOUND: i64 = -32601;
pub(crate) const INVALID_PARAMS: i64 = -32602;
pub(crate) const INTERNAL_ERROR: i64 = -32603;

/// What a caller may see of a decision it may not read whole: where it
/// stands, never what it holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetaDecisionStatus {
    pub decision_id: String,
    pub kind: String,
    pub phase: WireDecisionPhase,
    pub deadline_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at_ms: Option<u64>,
}

impl MetaDecisionStatus {
    pub(crate) fn of(row: &StoredDecision) -> Self {
        Self {
            decision_id: row.decision_id.to_string(),
            kind: row.descriptor.kind.to_string(),
            phase: wire_decision_phase(row.state.phase),
            deadline_at_ms: row.descriptor.deadline_at_ms,
            decided_at_ms: row.state.decided_at_ms,
            ended_at_ms: row.state.ended_at_ms,
        }
    }
}

/// One page of decisions: the rows the caller reads whole, the rows it may
/// see the status of, and the rows the server cannot read. `next_after`
/// names the decision id to read the next page after, and never a decision
/// the caller was not given: the store's cursor on the stdio host and for a
/// Console administrator, and for any other Console caller the last row of
/// the page, only while more of what it may see may follow.
#[derive(Debug, Clone, Serialize)]
pub struct MetaDecisionListResult {
    pub decisions: Vec<WireDecision>,
    pub statuses: Vec<MetaDecisionStatus>,
    pub unreadable: Vec<WireUnreadableDecision>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_after: Option<String>,
}

/// One decision as the caller may see it: whole (`{"decision": ...}`) or its
/// status (`{"status": ...}`). The whole decision is boxed: it is many times
/// the size of a status.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MetaDecisionView {
    Decision(Box<WireDecision>),
    Status(MetaDecisionStatus),
}

/// A host attestation of the person who decided, carried by a stdio
/// `meta/decisions/decide` for an ingress the launch declares: the subject
/// the ingress authenticated, its authentication scope (for example the
/// chat session), the ingress's reference of the decide event, and the time
/// the ingress observed it. `presentation` is what the ingress showed the
/// person, when it showed the decision itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionAttestationRequest {
    pub ingress: String,
    pub subject: String,
    pub scope: String,
    pub event_ref: String,
    pub observed_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation: Option<DecisionPresentationRequest>,
}

/// `meta/decisions/decide`. The request names no approver, actor,
/// assurance or scope: on the stdio host the operator decides, or the
/// person a declared ingress attests in `attestation`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaDecisionDecideParams {
    pub decision_id: String,
    pub approve: bool,
    pub presented_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attestation: Option<DecisionAttestationRequest>,
}

/// What a `meta/decisions/notice` says happened. Tolerant: a host decodes an
/// event a later version adds as `Unknown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetaDecisionNoticeEvent {
    /// An open decision the host has not been told of: nothing of its
    /// content is shown.
    Held,
    /// The decision shown to the stdio operator, recorded as presented on
    /// this connection under the notice's id.
    Presented,
    /// Forward compatibility: an event this version does not know. The
    /// server never sends it.
    #[serde(other)]
    Unknown,
}

/// `meta/decisions/notice`: one open decision, noticed once per stdio
/// connection. `rendering_ref` is the subject digest of the held rendering;
/// only a `presented` notice carries the rendering itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaDecisionNotice {
    pub notice_id: String,
    pub decision_id: String,
    pub kind: String,
    pub event: MetaDecisionNoticeEvent,
    pub phase: WireDecisionPhase,
    pub approver_id: String,
    pub requester: String,
    pub rendering_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation: Option<WireDecisionNoticePresentation>,
}

/// A successful JSON-RPC answer.
pub(crate) fn success(id: &Value, result: impl Serialize) -> Value {
    match serde_json::to_value(result) {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(error) => failure(
            id,
            INTERNAL_ERROR,
            format!("failed to serialize the decision answer: {error}"),
            None,
        ),
    }
}

/// A failed JSON-RPC answer.
pub(crate) fn failure(
    id: &Value,
    code: i64,
    message: impl Into<String>,
    data: Option<Value>,
) -> Value {
    let mut error = json!({"code": code, "message": message.into()});
    if let (Some(data), Some(object)) = (data, error.as_object_mut()) {
        object.insert("data".to_string(), data);
    }
    json!({"jsonrpc": "2.0", "id": id, "error": error})
}

/// Invalid params with no `data`: a request value this launch does not
/// accept, Core's documented form.
pub(crate) fn invalid_params(id: &Value, message: impl Into<String>) -> Value {
    failure(id, INVALID_PARAMS, message, None)
}

/// A typed decision answer: `code` and `data.kind`.
pub(crate) fn typed_failure(
    id: &Value,
    code: i64,
    message: impl Into<String>,
    data: WireDecisionErrorData,
) -> Value {
    match serde_json::to_value(data) {
        Ok(data) => failure(id, code, message, Some(data)),
        Err(error) => failure(
            id,
            INTERNAL_ERROR,
            format!("failed to serialize the decision error: {error}"),
            None,
        ),
    }
}

/// The answer of a runtime that keeps no durable decisions.
pub(crate) fn no_decision_store(id: &Value) -> Value {
    typed_failure(
        id,
        i64::from(ErrorCode::CapabilityUnavailable.jsonrpc_code()),
        "this runtime keeps no durable decisions",
        WireDecisionErrorData::NoDecisionStore,
    )
}

/// The answer to a decision that is not there, or that the caller may not
/// see at all: the two read alike.
pub(crate) fn not_found(id: &Value) -> Value {
    typed_failure(
        id,
        INVALID_PARAMS,
        "no decision has this id",
        WireDecisionErrorData::DecisionNotFound,
    )
}

/// The answer of a failed decision call, as Core's `decision/*` gives it:
/// a refusal and a missing decision are typed (`data.kind`); store faults
/// keep their commitment truth apart (nothing committed, another owner won,
/// or the write may have committed).
pub(crate) fn decision_error(id: &Value, error: DecisionError) -> Value {
    let message = error.to_string();
    match error {
        DecisionError::NotFound(_) => not_found(id),
        DecisionError::Refused { refusal, .. } => typed_failure(
            id,
            INVALID_PARAMS,
            message,
            WireDecisionErrorData::DecisionRefused {
                decision_refusal: wire_decision_refusal(refusal),
            },
        ),
        DecisionError::Unreadable { .. } => typed_failure(
            id,
            INTERNAL_ERROR,
            message,
            WireDecisionErrorData::DecisionUnreadable,
        ),
        DecisionError::CandidateLaunch => typed_failure(
            id,
            i64::from(ErrorCode::CapabilityUnavailable.jsonrpc_code()),
            message,
            WireDecisionErrorData::CandidateLaunch,
        ),
        DecisionError::Unavailable(_) | DecisionError::Unsettled(_) => failure(
            id,
            i64::from(ErrorCode::ApprovalStoreUnavailable.jsonrpc_code()),
            message,
            None,
        ),
        DecisionError::Conflict(_) => failure(
            id,
            i64::from(ErrorCode::ApprovalConflict.jsonrpc_code()),
            message,
            None,
        ),
        DecisionError::CommitUnknown(_) => failure(
            id,
            i64::from(ErrorCode::ApprovalCommitUnknown.jsonrpc_code()),
            message,
            None,
        ),
        DecisionError::Invalid(_) | DecisionError::NotAReceipt => {
            failure(id, INVALID_PARAMS, message, None)
        }
        // The clock, the machine or an executor claim: the server's fault.
        _ => failure(id, INTERNAL_ERROR, message, None),
    }
}

/// The notification line of `notice`.
pub(crate) fn notice_line(notice: &MetaDecisionNotice) -> Result<String, serde_json::Error> {
    serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "method": DECISION_NOTICE_METHOD,
        "params": notice,
    }))
}

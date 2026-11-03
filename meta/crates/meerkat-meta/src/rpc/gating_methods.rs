//! Parameter parsing for gating RPC methods.

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum GatingParamsError {
    ParamsMustBeObject,
    ActionRequired,
    ActorRequired,
    RiskTierRequired,
    UnknownRiskTier(String),
    RationaleMustBeString,
    RequestedApproverMustBeString,
    ApprovalRecipientMustBeString,
    ApprovalChannelMustBeString,
    ApprovalTimeoutMustBeInteger,
    EntityMustBeString,
    TopicMustBeString,
    PendingIdRequired,
    ApproverIdRequired,
    DecisionRequired,
    UnknownDecision(String),
    ReasonMustBeString,
    LimitOutOfRange,
    AfterAuditIdMustBeString,
    Decision(GatingDecideError),
}

impl GatingParamsError {
    pub(super) fn message(&self) -> String {
        match self {
            GatingParamsError::ParamsMustBeObject => "params must be a JSON object".to_string(),
            GatingParamsError::ActionRequired => "action must be a non-empty string".to_string(),
            GatingParamsError::ActorRequired => "actor_id must be a non-empty string".to_string(),
            GatingParamsError::RiskTierRequired => {
                "risk_tier must be one of: r0, r1, r2, r3".to_string()
            }
            GatingParamsError::UnknownRiskTier(tier) => {
                format!("risk_tier '{tier}' is unsupported (allowed: r0, r1, r2, r3)")
            }
            GatingParamsError::RationaleMustBeString => "rationale must be a string".to_string(),
            GatingParamsError::RequestedApproverMustBeString => {
                "requested_approver must be a string".to_string()
            }
            GatingParamsError::ApprovalRecipientMustBeString => {
                "approval_recipient must be a string".to_string()
            }
            GatingParamsError::ApprovalChannelMustBeString => {
                "approval_channel must be a string".to_string()
            }
            GatingParamsError::ApprovalTimeoutMustBeInteger => {
                "approval_timeout_ms must be a non-negative integer".to_string()
            }
            GatingParamsError::EntityMustBeString => {
                "entity must be a string when provided".to_string()
            }
            GatingParamsError::TopicMustBeString => {
                "topic must be a string when provided".to_string()
            }
            GatingParamsError::PendingIdRequired => {
                "pending_id must be a non-empty string".to_string()
            }
            GatingParamsError::ApproverIdRequired => {
                "approver_id must be a non-empty string".to_string()
            }
            GatingParamsError::DecisionRequired => {
                format!("decision must be one of: {}", gating_decision_names())
            }
            GatingParamsError::UnknownDecision(decision) => {
                format!(
                    "decision '{decision}' is unsupported (allowed: {})",
                    gating_decision_names()
                )
            }
            GatingParamsError::ReasonMustBeString => "reason must be a string".to_string(),
            GatingParamsError::LimitOutOfRange => {
                "limit must be an integer between 1 and 500".to_string()
            }
            GatingParamsError::AfterAuditIdMustBeString => {
                "after_audit_id must be a non-empty string when provided".to_string()
            }
            GatingParamsError::Decision(GatingDecideError::UnknownPendingId(pending_id)) => {
                format!("pending_id '{pending_id}' was not found")
            }
            GatingParamsError::Decision(GatingDecideError::SelfApprovalForbidden) => {
                "approver_id cannot self-approve the action actor".to_string()
            }
            GatingParamsError::Decision(GatingDecideError::ApproverMismatch {
                expected,
                provided,
            }) => {
                format!("approver_id '{provided}' does not match requested_approver '{expected}'")
            }
            GatingParamsError::Decision(GatingDecideError::EmptyPrincipal) => {
                "deciding principal must not be empty".to_string()
            }
            GatingParamsError::Decision(GatingDecideError::NoEscalationRoute {
                requested_approver,
            }) => match requested_approver {
                Some(approver) => format!("no escalation route is configured from '{approver}'"),
                None => "no default escalation route is configured".to_string(),
            },
            GatingParamsError::Decision(GatingDecideError::IdsUnavailable(reason)) => {
                format!("gating decision cannot be recorded: {reason}")
            }
        }
    }
}

/// JSON-RPC error for a gating decision the owner could not record because
/// it cannot mint gate identities (no epoch, or an exhausted sequence). This
/// is owner infrastructure, not invalid caller input, so it is an internal
/// error with a machine-readable reason.
pub(crate) fn gating_ids_unavailable_rpc_error(
    reason: crate::runtime::GatingIdUnavailable,
) -> JsonRpcError {
    JsonRpcError {
        code: -32603,
        message: format!("Internal error: {reason}"),
        data: Some(serde_json::json!({
            "error": "gating_ids_unavailable",
            "reason": reason.fallback_reason(),
        })),
    }
}

/// JSON-RPC error for a failed `gating/decide`: owner infrastructure failures
/// are internal errors; everything else is invalid params, as before.
pub(super) fn gating_decide_rpc_error(err: GatingParamsError) -> JsonRpcError {
    match err {
        GatingParamsError::Decision(GatingDecideError::IdsUnavailable(reason)) => {
            gating_ids_unavailable_rpc_error(reason)
        }
        err => JsonRpcError {
            code: -32602,
            message: format!("Invalid params: {}", err.message()),
            data: None,
        },
    }
}

pub(super) fn parse_gating_evaluate_params(
    params: &Value,
) -> Result<GatingEvaluateRequest, GatingParamsError> {
    let object = params
        .as_object()
        .ok_or(GatingParamsError::ParamsMustBeObject)?;
    let action = object
        .get("action")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(GatingParamsError::ActionRequired)?;
    let actor_id = object
        .get("actor_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(GatingParamsError::ActorRequired)?;
    let risk_tier = object
        .get("risk_tier")
        .and_then(Value::as_str)
        .ok_or(GatingParamsError::RiskTierRequired)?;
    let risk_tier = parse_gating_risk_tier(risk_tier)?;
    let rationale = match object.get("rationale") {
        None => None,
        Some(value) => Some(
            value
                .as_str()
                .ok_or(GatingParamsError::RationaleMustBeString)?
                .to_string(),
        ),
    };
    let requested_approver = match object.get("requested_approver") {
        None => None,
        Some(value) => Some(
            value
                .as_str()
                .ok_or(GatingParamsError::RequestedApproverMustBeString)?
                .to_string(),
        ),
    };
    let approval_recipient = match object.get("approval_recipient") {
        None => None,
        Some(value) => Some(
            value
                .as_str()
                .ok_or(GatingParamsError::ApprovalRecipientMustBeString)?
                .to_string(),
        ),
    };
    let approval_channel = match object.get("approval_channel") {
        None => None,
        Some(value) => Some(
            value
                .as_str()
                .ok_or(GatingParamsError::ApprovalChannelMustBeString)?
                .to_string(),
        ),
    };
    let approval_timeout_ms = match object.get("approval_timeout_ms") {
        None => None,
        Some(value) => Some(
            value
                .as_u64()
                .ok_or(GatingParamsError::ApprovalTimeoutMustBeInteger)?,
        ),
    };
    let entity = match object.get("entity") {
        None => None,
        Some(value) => Some(
            value
                .as_str()
                .ok_or(GatingParamsError::EntityMustBeString)?
                .to_string(),
        ),
    };
    let topic = match object.get("topic") {
        None => None,
        Some(value) => Some(
            value
                .as_str()
                .ok_or(GatingParamsError::TopicMustBeString)?
                .to_string(),
        ),
    };

    Ok(GatingEvaluateRequest {
        action: action.to_string(),
        actor_id: actor_id.to_string(),
        risk_tier,
        rationale,
        requested_approver,
        approval_recipient,
        approval_channel,
        approval_timeout_ms,
        entity,
        topic,
    })
}

pub(super) fn parse_gating_pending_params(params: &Value) -> Result<(), GatingParamsError> {
    if params.is_null() || params.is_object() {
        return Ok(());
    }
    Err(GatingParamsError::ParamsMustBeObject)
}

pub(super) fn parse_gating_decide_params(
    params: &Value,
) -> Result<GatingDecideRequest, GatingParamsError> {
    let object = params
        .as_object()
        .ok_or(GatingParamsError::ParamsMustBeObject)?;
    let pending_id = object
        .get("pending_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(GatingParamsError::PendingIdRequired)?;
    let approver_id = object
        .get("approver_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(GatingParamsError::ApproverIdRequired)?;
    let decision = object
        .get("decision")
        .and_then(Value::as_str)
        .ok_or(GatingParamsError::DecisionRequired)?;
    let decision = parse_gating_decision(decision)?;
    let reason = match object.get("reason") {
        None => None,
        Some(value) => Some(
            value
                .as_str()
                .ok_or(GatingParamsError::ReasonMustBeString)?
                .to_string(),
        ),
    };

    Ok(GatingDecideRequest {
        pending_id: pending_id.to_string(),
        approver_id: approver_id.to_string(),
        decision,
        reason,
    })
}

/// `meta/gating/audit` parameters: a limit, and an optional cursor.
pub(super) struct GatingAuditParams {
    pub(super) limit: usize,
    pub(super) after_audit_id: Option<String>,
}

pub(super) fn parse_gating_audit_params(
    params: &Value,
) -> Result<GatingAuditParams, GatingParamsError> {
    let limit = parse_gating_audit_limit(params)?;
    let after_audit_id = parse_after_audit_id(params)?;
    Ok(GatingAuditParams {
        limit,
        after_audit_id,
    })
}

/// The `meta/gating/audit` result: with a cursor, up to `limit` entries
/// after it; without one, the newest `limit` entries. `next_cursor` is the
/// last returned id, the given cursor when nothing new was returned, or null
/// after a gap with nothing returned (the stale cursor is not echoed).
pub(super) fn gating_audit_result(
    entries: Vec<crate::runtime::GatingAuditEntry>,
    gap: bool,
    after_audit_id: Option<&str>,
) -> Value {
    let next_cursor = entries
        .last()
        .map(|entry| entry.audit_id.clone())
        .or_else(|| {
            (!gap)
                .then(|| after_audit_id.map(ToString::to_string))
                .flatten()
        });
    serde_json::json!({
        "entries": entries,
        "next_cursor": next_cursor,
        "gap": gap,
    })
}

/// The optional audit cursor, refused when present but not a non-empty
/// string. Shared by the console route.
pub(crate) fn parse_gating_audit_cursor(params: &Value) -> Result<Option<String>, String> {
    parse_after_audit_id(params).map_err(|error| error.message())
}

fn parse_after_audit_id(params: &Value) -> Result<Option<String>, GatingParamsError> {
    match params.get("after_audit_id") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .map(str::trim)
            .filter(|cursor| !cursor.is_empty())
            .map(|cursor| Some(cursor.to_string()))
            .ok_or(GatingParamsError::AfterAuditIdMustBeString),
    }
}

fn parse_gating_audit_limit(params: &Value) -> Result<usize, GatingParamsError> {
    if params.is_null() {
        return Ok(50);
    }
    let object = params
        .as_object()
        .ok_or(GatingParamsError::ParamsMustBeObject)?;
    let limit = match object.get("limit") {
        None => 50,
        Some(value) => {
            let Some(raw_limit) = value.as_u64() else {
                return Err(GatingParamsError::LimitOutOfRange);
            };
            if !(1..=500).contains(&raw_limit) {
                return Err(GatingParamsError::LimitOutOfRange);
            }
            raw_limit as usize
        }
    };
    Ok(limit)
}

fn parse_gating_risk_tier(raw: &str) -> Result<GatingRiskTier, GatingParamsError> {
    let normalized = raw.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "r0" => Ok(GatingRiskTier::R0),
        "r1" => Ok(GatingRiskTier::R1),
        "r2" => Ok(GatingRiskTier::R2),
        "r3" => Ok(GatingRiskTier::R3),
        "" => Err(GatingParamsError::RiskTierRequired),
        _ => Err(GatingParamsError::UnknownRiskTier(raw.to_string())),
    }
}

fn parse_gating_decision(raw: &str) -> Result<GatingDecision, GatingParamsError> {
    let normalized = raw.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Err(GatingParamsError::DecisionRequired);
    }
    GATING_DECISIONS
        .iter()
        .find(|(name, _)| *name == normalized)
        .map(|(_, decision)| decision.clone())
        .ok_or_else(|| GatingParamsError::UnknownDecision(raw.to_string()))
}

/// Every decision `gating/decide` accepts, by its wire name: the one list
/// the parser reads and both decision errors name.
const GATING_DECISIONS: [(&str, GatingDecision); 3] = [
    ("approve", GatingDecision::Approve),
    ("reject", GatingDecision::Reject),
    ("escalate", GatingDecision::Escalate),
];

/// The accepted decisions as the errors list them.
fn gating_decision_names() -> String {
    GATING_DECISIONS
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `gating/decide` accepts each decision by its wire name, and both of
    /// its decision errors list every one of them.
    #[test]
    fn every_decision_is_accepted_by_its_wire_name_and_listed_in_the_errors() {
        for decision in [
            GatingDecision::Approve,
            GatingDecision::Reject,
            GatingDecision::Escalate,
        ] {
            // Exhaustive: a new decision fails to compile here until its
            // wire name is accepted and listed.
            let wire = match decision {
                GatingDecision::Approve => "approve",
                GatingDecision::Reject => "reject",
                GatingDecision::Escalate => "escalate",
            };
            assert_eq!(
                serde_json::to_value(&decision).ok(),
                Some(serde_json::json!(wire))
            );
            assert_eq!(
                parse_gating_decision(&format!(" {} ", wire.to_ascii_uppercase())),
                Ok(decision)
            );
        }
        assert_eq!(
            parse_gating_decision("defer").map_err(|error| error.message()),
            Err("decision 'defer' is unsupported (allowed: approve, reject, escalate)".to_string())
        );
        assert_eq!(
            parse_gating_decision(" ").map_err(|error| error.message()),
            Err("decision must be one of: approve, reject, escalate".to_string())
        );
    }
}

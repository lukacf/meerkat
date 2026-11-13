//! Question sets on the stdio gating methods: `meta/gating/questions`, and
//! the branch of `meta/gating/decide` whose `pending_id` names a question
//! set (`crate::ask_user`).
//!
//! The host reads every open set, as it reads every pending approval. A
//! decide is the host's: `approver_id` is required as for every
//! `meta/gating/decide` and names who the host vouches for; the decision is
//! recorded as the host operator's, device trusted, on the ingress `host`,
//! and the host is eligible as the stdio gating decide trusts its host.

use meerkat_core::approval::ApprovalAssurance;
use meerkat_core::approval::consent::{
    AuthenticatedApprover, ConsentEligibility, ConsentIneligible,
};
use meerkat_core::approval::decision::{DecisionId, DecisionReference};
use meerkat_runtime::decision::{DecisionClock, DecisionRefusal, SystemDecisionClock};

use super::*;
use crate::ask_user::{
    HOST_CHANNEL, HeldQuestionSet, QuestionSetChoice, QuestionSetDecide, QuestionSetEvent,
    QuestionSetRefusal, answer_question_set, held_question_set, open_question_sets,
    question_set_request, question_sets_result,
};
use crate::durable_decisions::RuntimeDecisions;
use crate::runtime::GatingDecision;

/// The host's own eligibility: the stdio gating decide trusts its host.
struct HostEligible;

impl ConsentEligibility for HostEligible {
    fn check_current(&self) -> Result<(), ConsentIneligible> {
        Ok(())
    }
}

fn answered(response_id: Value, result: Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: response_id,
        result: Some(result),
        error: None,
    }
}

fn refused(response_id: Value, code: i64, message: impl Into<String>) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: response_id,
        result: None,
        error: Some(JsonRpcError {
            code,
            message: message.into(),
            data: None,
        }),
    }
}

fn invalid(response_id: Value, message: impl std::fmt::Display) -> JsonRpcResponse {
    refused(response_id, -32602, format!("Invalid params: {message}"))
}

/// `meta/gating/questions` for the host: every open question set of
/// `decisions`, soonest deadline first; none for a runtime that holds none.
pub(crate) fn gating_questions_response(
    decisions: Option<&RuntimeDecisions>,
    params: &Value,
    response_id: Value,
) -> JsonRpcResponse {
    if let Err(err) = parse_gating_pending_params(params) {
        return invalid(response_id, err.message());
    }
    let sets = match decisions {
        Some(decisions) => open_question_sets(decisions),
        None => Ok(Vec::new()),
    };
    match sets {
        Ok(sets) => answered(response_id, question_sets_result(&sets)),
        Err(error) => refused(
            response_id,
            -32603,
            format!("the question sets could not be read: {error}"),
        ),
    }
}

/// The question set the `pending_id` of a decide's `params` names in the
/// runtime's durable decisions, when it names one.
fn named_question_set(
    runtime: &crate::mob_handle_runtime::MobRuntime,
    params: &Value,
) -> Option<(RuntimeDecisions, HeldQuestionSet)> {
    let pending_id = params.get("pending_id").and_then(Value::as_str)?.trim();
    let decisions = runtime.ask_user_slot().decisions()?;
    let set = held_question_set(&decisions, pending_id)?;
    Some((decisions, set))
}

/// Whether a `meta/gating/decide` with `params` names a question set. Any
/// other id, a decision of another kind included, is the gating owner's to
/// answer, as an unknown pending id.
pub(crate) fn gating_decide_names_question_set(
    runtime: &crate::mob_handle_runtime::MobRuntime,
    params: &Value,
) -> bool {
    named_question_set(runtime, params).is_some()
}

/// `meta/gating/decide` for a question set, as the host: `approve` with
/// `answers` answers it, `reject` or `deny` dismisses it.
pub(crate) fn gating_decide_question_set(
    runtime: &crate::mob_handle_runtime::MobRuntime,
    params: &Value,
    response_id: Value,
) -> JsonRpcResponse {
    let Some((decisions, set)) = named_question_set(runtime, params) else {
        return invalid(response_id, "pending_id names no question set");
    };
    // The request is read as every gating decide is: `approver_id` is
    // required, and the same refusals answer what it lacks.
    let request = match parse_gating_decide_params(params) {
        Ok(request) => request,
        Err(err) => {
            return JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: None,
                error: Some(gating_decide_rpc_error(err)),
            };
        }
    };
    let choice = match request.decision {
        GatingDecision::Approve => QuestionSetChoice::Answer,
        GatingDecision::Reject => QuestionSetChoice::Dismiss,
        GatingDecision::Escalate => {
            return invalid(response_id, "a question set cannot be escalated");
        }
    };
    let (verdict, presented_digest) = match question_set_request(choice, params) {
        Ok(request) => request,
        Err(message) => return invalid(response_id, message),
    };
    let operator = decisions
        .realm()
        .ok_or_else(|| "the runtime names no realm".to_owned())
        .and_then(crate::durable_decisions::local_operator_principal)
        .and_then(|principal| {
            AuthenticatedApprover::new(principal, ApprovalAssurance::DeviceTrusted)
                .map_err(|error| error.to_string())
        });
    let approver = match operator {
        Ok(approver) => approver,
        Err(error) => {
            return refused(
                response_id,
                -32603,
                format!("the host cannot be named as the decider: {error}"),
            );
        }
    };
    let observed_at_ms = match SystemDecisionClock.now_ms() {
        Ok(now_ms) => now_ms,
        Err(error) => return refused(response_id, -32603, error.to_string()),
    };
    let event_ref = match DecisionReference::new(format!("meta-gating:{}", uuid::Uuid::new_v4())) {
        Ok(event_ref) => event_ref,
        Err(error) => return refused(response_id, -32603, error.to_string()),
    };
    // Who the host vouches for is kept with the evidence, with the reason
    // the host gave, if any.
    let vouched = format!("the host answered for {}", request.approver_id);
    let reason = match request.reason {
        Some(reason) => format!("{vouched}: {reason}"),
        None => vouched,
    };
    let decide = QuestionSetDecide {
        decision_id: set.decision_id().clone(),
        verdict,
        presented_digest,
        attested: QuestionSetEvent {
            approver,
            channel: HOST_CHANNEL,
            event_ref,
            observed_at_ms,
            reason: Some(reason),
        },
    };
    match answer_question_set(&decisions, decide, &HostEligible) {
        Ok(decided) => answered(response_id, decided.wire()),
        Err(QuestionSetRefusal::Invalid(message)) => invalid(response_id, message),
        Err(QuestionSetRefusal::Refused(refusal)) => {
            refused_by_decision(response_id, set.decision_id(), refusal)
        }
        Err(QuestionSetRefusal::NotEligible) => {
            refused_by_decision(response_id, set.decision_id(), DecisionRefusal::NotEligible)
        }
        // The read found the set a moment ago: it is no longer one this
        // route decides.
        Err(QuestionSetRefusal::NotAQuestionSet) => {
            refused_by_decision(response_id, set.decision_id(), DecisionRefusal::NotOpen)
        }
        Err(QuestionSetRefusal::Failed(error)) => refused(
            response_id,
            -32603,
            format!("the question set could not be decided: {error}"),
        ),
    }
}

/// The decision's own refusal of a decide, in the typed form the decision
/// methods give it (`data.kind = "decision_refused"` with the refusal).
fn refused_by_decision(
    response_id: Value,
    decision_id: &DecisionId,
    refusal: DecisionRefusal,
) -> JsonRpcResponse {
    let answer = crate::ask_user::refused_answer(&response_id, decision_id, refusal);
    serde_json::from_value(answer).unwrap_or_else(|error| {
        refused(
            response_id,
            -32603,
            format!("the decision's refusal could not be encoded: {error}"),
        )
    })
}

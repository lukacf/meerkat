//! Question sets on the Console: the `meta/gating/questions` read, and the
//! branch of `meta/gating/decide` whose `pending_id` names a question set
//! (`crate::ask_user`).
//!
//! The read lists the open sets whose asking member the caller may see, by
//! the rule the `meta/gating/pending` read applies to an entry's origin
//! ([`gating_origin_visible`]): a set of a member private to someone else
//! is not listed.
//!
//! The decide keeps, in order, every check of an approval's decide. The
//! arm refuses a request that names an approver before it branches. Then:
//! the asking member must be one the caller may view (`access_denied`,
//! "access denied to approval origin"); the caller must be a verified
//! person ([`ConsoleApprovalDecider`], `approver_unauthenticated`); the
//! decision is `approve` with answers, or `reject`/`deny`
//! ([`console_gating_decision`]). The person's decision is then handed to
//! the decision owner ([`crate::ask_user::answer_question_set`]) as one
//! attested human decision: person authenticated, in the login it was made
//! in, on the ingress `console`. The checks an approval's decide makes
//! under the access fence ([`console_decide_fenced`]: the person is still
//! current, still holds `gating.decide`, and may still view the asking
//! member) are the eligibility the owner checks when the decision commits,
//! with the fence held through that call, so a revocation published while
//! the request waited is honored.

use std::sync::OnceLock;

use meerkat_core::approval::ApprovalAssurance;
use meerkat_core::approval::consent::{
    AuthenticatedApprover, ConsentEligibility, ConsentIneligible,
};
use meerkat_core::approval::decision::{DecisionIngress, DecisionReference};
use meerkat_runtime::decision::{DecisionClock, SystemDecisionClock};

use super::*;
use crate::access::AccessDecisionFence;
use crate::ask_user::{
    CONSOLE_CHANNEL, HeldQuestionSet, QuestionSetChoice, QuestionSetDecide, QuestionSetEvent,
    QuestionSetRefusal, answer_question_set, held_question_set, open_question_sets,
    question_set_request, question_sets_result,
};
use crate::durable_decisions::{DecisionEventBinding, RuntimeDecisions};

fn internal_error(response_id: Value, message: String) -> Value {
    response_value(
        response_id,
        None,
        Some(JsonRpcError {
            code: -32603,
            message,
            data: None,
        }),
    )
}

/// `meta/gating/questions`: the open question sets of the runtime's members
/// the caller may see, soonest deadline first.
pub(super) fn list(
    runtime: &MobRuntime,
    access_view: Option<&AccessView>,
    response_id: Value,
) -> Value {
    let sets = match runtime.ask_user_slot().decisions() {
        Some(decisions) => open_question_sets(&decisions),
        None => Ok(Vec::new()),
    };
    match sets {
        Ok(sets) => response_value(
            response_id,
            Some(question_sets_result(sets.iter().filter(|set| {
                gating_origin_visible(access_view, set.asker())
            }))),
            None,
        ),
        Err(error) => internal_error(
            response_id,
            format!("the question sets could not be read: {error}"),
        ),
    }
}

/// The question set `pending_id` names in the runtime's durable decisions,
/// when it names one. Any other id, a decision of another kind included, is
/// the gating owner's to answer, as an unknown pending id.
pub(super) fn named(
    runtime: &MobRuntime,
    pending_id: &str,
) -> Option<(RuntimeDecisions, HeldQuestionSet)> {
    let decisions = runtime.ask_user_slot().decisions()?;
    let set = held_question_set(&decisions, pending_id)?;
    Some((decisions, set))
}

/// The person who decides, eligible while the checks of an approval's
/// decide pass under the access fence. The refusal of the check that
/// failed is kept for the route's answer.
struct FencedPerson<'a, 'fence> {
    decider: &'a ConsoleApprovalDecider,
    fence: Option<&'a AccessDecisionFence<'fence>>,
    asker: &'a str,
    refused: OnceLock<ConsoleDecideRefusal>,
}

impl ConsentEligibility for FencedPerson<'_, '_> {
    fn check_current(&self) -> Result<(), ConsentIneligible> {
        match console_decide_fenced(self.decider, self.fence, Some(self.asker)) {
            Ok(()) => Ok(()),
            Err(refusal) => {
                let _ = self.refused.set(refusal);
                Err(ConsentIneligible)
            }
        }
    }
}

/// `meta/gating/decide` for the question set `set`, after the arm refused a
/// request that names an approver: see the module documentation.
#[allow(clippy::too_many_arguments)]
pub(super) async fn decide(
    decisions: RuntimeDecisions,
    set: HeldQuestionSet,
    params: &Value,
    response_id: Value,
    access: Option<&AccessController>,
    access_view: Option<&AccessView>,
    governance: &ConsoleSendGovernance,
    authenticated_principal: Option<&str>,
) -> Value {
    if access_view.is_some_and(|view| !view.can_view_agent(set.asker())) {
        return access_denied_rpc_error(response_id, "access denied to approval origin");
    }
    let Some(decider) =
        ConsoleApprovalDecider::of(governance, authenticated_principal, access_view)
    else {
        return approver_unauthenticated_rpc_error(response_id);
    };
    let Some(raw_decision) = params.get("decision").and_then(Value::as_str) else {
        return invalid_params(response_id, "decision required");
    };
    let choice = match console_gating_decision(raw_decision) {
        Ok(GatingDecision::Approve) => QuestionSetChoice::Answer,
        Ok(GatingDecision::Reject) => QuestionSetChoice::Dismiss,
        Ok(GatingDecision::Escalate) => {
            return invalid_params(response_id, "a question set cannot be escalated");
        }
        Err(message) => return invalid_params(response_id, message),
    };
    let (verdict, presented_digest) = match question_set_request(choice, params) {
        Ok(request) => request,
        Err(message) => return invalid_params(response_id, message),
    };
    // The person, authenticated in the login the request was made in.
    let approver = decider
        .human
        .principal()
        .map_err(|error| error.to_string())
        .and_then(|principal| {
            AuthenticatedApprover::new(principal, ApprovalAssurance::PersonAuthenticated)
                .map_err(|error| error.to_string())
        });
    let approver = match approver {
        Ok(approver) => approver.with_authentication_scope(decider.human.authentication_scope()),
        Err(_) => return approver_unauthenticated_rpc_error(response_id),
    };
    let observed_at_ms = match SystemDecisionClock.now_ms() {
        Ok(now_ms) => now_ms,
        Err(error) => return internal_error(response_id, error.to_string()),
    };
    let event = DecisionIngress::new(CONSOLE_CHANNEL).and_then(|ingress| {
        Ok((
            ingress,
            DecisionReference::new(format!("meta-console:{}", uuid::Uuid::new_v4()))?,
        ))
    });
    let (ingress, event_ref) = match event {
        Ok(event) => event,
        Err(error) => return internal_error(response_id, error.to_string()),
    };
    // The event is bound to its decision durably before the decide input.
    match decisions
        .bind_event(&ingress, &event_ref, set.decision_id())
        .await
    {
        Ok(DecisionEventBinding::Own) => {}
        Ok(DecisionEventBinding::Another) => {
            return internal_error(
                response_id,
                "the decision event was used by another decision".to_owned(),
            );
        }
        Err(error) => {
            return internal_error(
                response_id,
                format!("the decision event could not be recorded: {error}"),
            );
        }
    }
    // Test hook: the decide waits here, after the route's last await and
    // before the decision is handed over.
    #[cfg(test)]
    console_decide_holds::pass(set.decision_id().as_str()).await;
    let decide = QuestionSetDecide {
        decision_id: set.decision_id().clone(),
        verdict,
        presented_digest,
        attested: QuestionSetEvent {
            approver,
            channel: CONSOLE_CHANNEL,
            event_ref,
            observed_at_ms,
            reason: None,
        },
    };
    // The access fence is held through the hand-off: the owner checks the
    // person again when the decision commits.
    let fence = access.map(|controller| {
        controller.decision_fence(
            Some(&decider.principal),
            access_view.and_then(AccessView::private_owner_key),
        )
    });
    let eligibility = FencedPerson {
        decider: &decider,
        fence: fence.as_ref(),
        asker: set.asker(),
        refused: OnceLock::new(),
    };
    let decided = answer_question_set(&decisions, decide, &eligibility);
    let refused = eligibility.refused.into_inner();
    drop(fence);
    match decided {
        Ok(decided) => response_value(response_id, Some(decided.wire()), None),
        Err(QuestionSetRefusal::NotEligible) => match refused {
            Some(refusal) => refusal.into_rpc_error(response_id),
            None => invalid_params(response_id, "the question set is no longer open"),
        },
        Err(QuestionSetRefusal::Invalid(message)) => invalid_params(response_id, message),
        Err(QuestionSetRefusal::NotAQuestionSet) => {
            invalid_params(response_id, "the question set is no longer open")
        }
        Err(QuestionSetRefusal::Failed(error)) => internal_error(
            response_id,
            format!("the question set could not be decided: {error}"),
        ),
    }
}
